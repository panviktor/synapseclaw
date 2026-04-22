use crate::domain::message::ChatMessage;

const MAX_PARENT_SUMMARY_CHARS: usize = 360;
const MAX_RECENT_CALL_TURNS: usize = 4;
const MAX_RECENT_CALL_TURN_CHARS: usize = 180;
const MAX_HISTORY_NOTE_CHARS: usize = 420;
const MAX_MERGED_SUMMARY_CHARS: usize = 640;
const HISTORY_NOTE_PREFIX: &str = "[Recent live call]\n";
const LATEST_LIVE_CALL_LABEL: &str = "Latest live call:\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRealtimeCallReturnHandoff {
    pub history_note: Option<String>,
    pub merged_summary: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LiveRealtimeCallReturnHandoffInput<'a> {
    pub existing_summary: Option<&'a str>,
    pub recent_call_turns: &'a [ChatMessage],
}

fn bounded_text(value: &str, limit: usize) -> Option<String> {
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

fn role_label(role: &str) -> Option<&'static str> {
    match role {
        "user" => Some("User"),
        "assistant" => Some("Assistant"),
        _ => None,
    }
}

fn recent_call_turn_lines(turns: &[ChatMessage]) -> Vec<String> {
    let mut selected = turns
        .iter()
        .rev()
        .filter_map(|turn| {
            let role = role_label(turn.role.as_str())?;
            let content = bounded_text(&turn.content, MAX_RECENT_CALL_TURN_CHARS)?;
            Some(format!("{role}: {content}"))
        })
        .take(MAX_RECENT_CALL_TURNS)
        .collect::<Vec<_>>();
    selected.reverse();
    selected
}

pub fn strip_live_realtime_call_section(summary: &str) -> &str {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        return trimmed;
    }
    if let Some((prefix, _)) = trimmed.rsplit_once(&format!("\n\n{LATEST_LIVE_CALL_LABEL}")) {
        return prefix.trim_end();
    }
    if trimmed.starts_with(LATEST_LIVE_CALL_LABEL) {
        return "";
    }
    trimmed
}

pub fn is_live_realtime_call_history_note_content(content: &str) -> bool {
    content.starts_with(HISTORY_NOTE_PREFIX)
}

pub fn build_live_realtime_call_return_handoff(
    input: LiveRealtimeCallReturnHandoffInput<'_>,
) -> LiveRealtimeCallReturnHandoff {
    let recent_lines = recent_call_turn_lines(input.recent_call_turns);
    if recent_lines.is_empty() {
        return LiveRealtimeCallReturnHandoff {
            history_note: None,
            merged_summary: None,
        };
    }

    let history_note = bounded_text(
        &format!("{HISTORY_NOTE_PREFIX}- {}", recent_lines.join("\n- ")),
        MAX_HISTORY_NOTE_CHARS,
    );

    let mut sections = Vec::new();
    if let Some(summary) = bounded_text(
        strip_live_realtime_call_section(input.existing_summary.unwrap_or("")),
        MAX_PARENT_SUMMARY_CHARS,
    ) {
        sections.push(summary);
    }
    sections.push(format!(
        "{LATEST_LIVE_CALL_LABEL}- {}",
        recent_lines.join("\n- ")
    ));

    LiveRealtimeCallReturnHandoff {
        history_note,
        merged_summary: bounded_text(&sections.join("\n\n"), MAX_MERGED_SUMMARY_CHARS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_history_note_and_summary_from_recent_call_turns() {
        let handoff = build_live_realtime_call_return_handoff(LiveRealtimeCallReturnHandoffInput {
            existing_summary: Some("Discussed trip options."),
            recent_call_turns: &[
                ChatMessage::user("What do you think about Batumi?"),
                ChatMessage::assistant("It fits your budget and timing."),
            ],
        });

        let history_note = handoff.history_note.expect("history note");
        assert!(history_note.starts_with("[Recent live call]"));
        assert!(history_note.contains("User: What do you think about Batumi?"));
        assert!(history_note.contains("Assistant: It fits your budget and timing."));

        let merged_summary = handoff.merged_summary.expect("merged summary");
        assert!(merged_summary.contains("Discussed trip options."));
        assert!(merged_summary.contains("Latest live call:"));
        assert!(merged_summary.contains("User: What do you think about Batumi?"));
    }

    #[test]
    fn replaces_previous_live_call_section_in_summary() {
        let handoff = build_live_realtime_call_return_handoff(LiveRealtimeCallReturnHandoffInput {
            existing_summary: Some(
                "Discussed trip options.\n\nLatest live call:\n- User: old\n- Assistant: old reply",
            ),
            recent_call_turns: &[
                ChatMessage::user("Any weather update?"),
                ChatMessage::assistant("It will be warm tomorrow."),
            ],
        });

        let merged_summary = handoff.merged_summary.expect("merged summary");
        assert!(merged_summary.contains("Discussed trip options."));
        assert!(merged_summary.contains("User: Any weather update?"));
        assert!(!merged_summary.contains("old reply"));
    }

    #[test]
    fn ignores_non_dialogue_turns() {
        let handoff = build_live_realtime_call_return_handoff(LiveRealtimeCallReturnHandoffInput {
            existing_summary: None,
            recent_call_turns: &[
                ChatMessage::system("system"),
                ChatMessage::tool("tool"),
                ChatMessage::assistant("Short answer."),
            ],
        });

        let history_note = handoff.history_note.expect("history note");
        assert!(history_note.contains("Assistant: Short answer."));
        assert!(!history_note.contains("system"));
        assert!(!history_note.contains("tool"));
    }

    #[test]
    fn detects_live_call_history_note_content() {
        assert!(is_live_realtime_call_history_note_content(
            "[Recent live call]\n- User: hello"
        ));
        assert!(!is_live_realtime_call_history_note_content("hello"));
    }
}
