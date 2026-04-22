use crate::config::schema::AgentLiveCallConfig;
use crate::domain::user_profile::UserProfile;
use crate::ports::realtime_call::RealtimeCallStartRequest;

const MAX_PROMPT_CHARS: usize = 480;
const MAX_OBJECTIVE_CHARS: usize = 240;
const MAX_CONTEXT_CHARS: usize = 480;
const MAX_AGENDA_ITEMS: usize = 6;
const MAX_AGENDA_ITEM_CHARS: usize = 160;
const DEFAULT_GREETING_FALLBACK: &str = "Hello. I'm here.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLiveRealtimeCallPolicy {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_tool_iterations: usize,
    pub max_spoken_chars: usize,
    pub max_spoken_sentences: usize,
    pub excluded_tools: Vec<String>,
    pub locale: String,
}

pub struct LiveRealtimeCallPolicyInput<'a> {
    pub config: &'a AgentLiveCallConfig,
    pub current_user_text: Option<&'a str>,
    pub chat_handoff: Option<&'a str>,
    pub user_profile: Option<&'a UserProfile>,
}

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

fn normalized_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_locale_tag(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.replace('_', "-").to_ascii_lowercase();
    let mut parts = normalized
        .split('-')
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    if parts.len() > 2 {
        parts.truncate(2);
    }
    Some(parts.join("-"))
}

fn base_locale_tag(value: &str) -> &str {
    value.split('-').next().unwrap_or(value)
}

fn infer_locale_from_text(text: &str) -> Option<String> {
    let mut alphabetic = 0usize;
    let mut cyrillic = 0usize;
    let mut kana = 0usize;
    let mut hangul = 0usize;
    let mut arabic = 0usize;
    let mut hebrew = 0usize;
    let mut devanagari = 0usize;
    let mut han = 0usize;

    for ch in text.chars() {
        if ch.is_alphabetic() {
            alphabetic += 1;
        }
        match ch {
            '\u{0400}'..='\u{052F}' | '\u{2DE0}'..='\u{2DFF}' | '\u{A640}'..='\u{A69F}' => {
                cyrillic += 1;
            }
            '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' => {
                kana += 1;
            }
            '\u{AC00}'..='\u{D7AF}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}' => {
                hangul += 1;
            }
            '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{08A0}'..='\u{08FF}' => {
                arabic += 1;
            }
            '\u{0590}'..='\u{05FF}' => {
                hebrew += 1;
            }
            '\u{0900}'..='\u{097F}' => {
                devanagari += 1;
            }
            '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2EBEF}' => {
                han += 1;
            }
            _ => {}
        }
    }

    if cyrillic > 0 {
        return Some("ru".into());
    }
    if kana > 0 {
        return Some("ja".into());
    }
    if hangul > 0 {
        return Some("ko".into());
    }
    if arabic > 0 {
        return Some("ar".into());
    }
    if hebrew > 0 {
        return Some("he".into());
    }
    if devanagari > 0 {
        return Some("hi".into());
    }
    if han > 0 {
        return Some("zh".into());
    }
    (alphabetic > 0).then(|| "en".into())
}

fn resolve_profile_locale(
    config: &AgentLiveCallConfig,
    user_profile: Option<&UserProfile>,
) -> Option<String> {
    user_profile
        .and_then(|profile| profile.get_text(&config.profile_locale_key))
        .and_then(|value| normalize_locale_tag(&value))
}

pub fn resolve_live_realtime_call_locale(
    config: &AgentLiveCallConfig,
    current_user_text: Option<&str>,
    chat_handoff: Option<&str>,
    user_profile: Option<&UserProfile>,
) -> String {
    infer_locale_from_text(current_user_text.unwrap_or_default())
        .or_else(|| resolve_profile_locale(config, user_profile))
        .or_else(|| infer_locale_from_text(chat_handoff.unwrap_or_default()))
        .or_else(|| normalize_locale_tag(&config.fallback_locale))
        .unwrap_or_else(|| "en".into())
}

pub fn resolve_live_realtime_call_policy(
    input: LiveRealtimeCallPolicyInput<'_>,
) -> ResolvedLiveRealtimeCallPolicy {
    let config = input.config;
    ResolvedLiveRealtimeCallPolicy {
        provider: normalized_non_empty(config.provider.as_deref()),
        model: normalized_non_empty(config.model.as_deref()),
        max_tool_iterations: config.max_tool_iterations.max(1),
        max_spoken_chars: config.max_spoken_chars.max(80),
        max_spoken_sentences: config.max_spoken_sentences.max(1),
        excluded_tools: config
            .excluded_tools
            .iter()
            .filter_map(|tool| normalized_non_empty(Some(tool)))
            .collect(),
        locale: resolve_live_realtime_call_locale(
            config,
            input.current_user_text,
            input.chat_handoff,
            input.user_profile,
        ),
    }
}

pub fn resolve_live_realtime_call_route(
    config: &AgentLiveCallConfig,
    default_provider: &str,
    default_model: &str,
) -> (String, String) {
    (
        normalized_non_empty(config.provider.as_deref())
            .unwrap_or_else(|| default_provider.trim().to_string()),
        normalized_non_empty(config.model.as_deref())
            .unwrap_or_else(|| default_model.trim().to_string()),
    )
}

pub fn build_live_realtime_call_system_prompt(
    base: &str,
    locale: &str,
    handoff: Option<&str>,
    max_tool_iterations: usize,
) -> String {
    let mut prompt = base.to_string();
    prompt.push_str(
        "\n\n[Live voice call mode]\nRespond immediately and speak naturally. Keep every reply short enough to say aloud comfortably. No lists, no headers, no long structured explanations.",
    );
    prompt.push_str(&format!(
        "\nPreferred response locale: {}. If the caller clearly switches language, follow the caller.",
        locale
    ));
    prompt.push_str(&format!(
        "\nTool policy: avoid tools unless they are strictly necessary for fresh external data or a concrete side effect. Keep live-call tool work within {} round(s). If one short clarification question is enough, ask it directly.",
        max_tool_iterations
    ));
    if let Some(handoff) = handoff.map(str::trim).filter(|value| !value.is_empty()) {
        prompt.push_str("\n\n[Chat-to-call handoff]\n");
        prompt.push_str(handoff);
    }
    prompt
}

pub fn merge_live_realtime_call_excluded_tools(
    global_excluded_tools: &[String],
    live_call_config: &AgentLiveCallConfig,
) -> Vec<String> {
    let mut merged = global_excluded_tools.to_vec();
    for tool_name in &live_call_config.excluded_tools {
        let Some(tool_name) = normalized_non_empty(Some(tool_name)) else {
            continue;
        };
        if !merged.iter().any(|existing| existing == &tool_name) {
            merged.push(tool_name);
        }
    }
    merged
}

pub fn bounded_live_realtime_call_speech(config: &AgentLiveCallConfig, text: &str) -> String {
    let max_sentences = config.max_spoken_sentences.max(1);
    let max_chars = config.max_spoken_chars.max(80);
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return String::new();
    }

    let mut selected = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in normalized.char_indices() {
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let candidate = normalized[start..=idx].trim();
            if !candidate.is_empty() {
                selected.push(candidate.to_string());
                if selected.len() >= max_sentences {
                    break;
                }
            }
            start = idx + ch.len_utf8();
        }
    }
    if selected.is_empty() {
        for line in normalized.lines() {
            let candidate = line.trim();
            if !candidate.is_empty() {
                selected.push(candidate.to_string());
                break;
            }
        }
    }
    if selected.is_empty() {
        return crate::domain::util::truncate_with_ellipsis(&normalized, max_chars);
    }

    let spoken = selected.join(" ");
    crate::domain::util::truncate_with_ellipsis(spoken.trim(), max_chars)
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

pub fn default_realtime_call_answer_greeting(
    config: &AgentLiveCallConfig,
    locale: Option<&str>,
) -> String {
    let requested_locale = locale
        .and_then(normalize_locale_tag)
        .or_else(|| normalize_locale_tag(&config.fallback_locale));

    let locale_candidates = requested_locale
        .as_deref()
        .map(|resolved| vec![resolved.to_string(), base_locale_tag(resolved).to_string()])
        .unwrap_or_else(|| vec!["en".into()]);

    for candidate in locale_candidates {
        if let Some(value) = config
            .greetings
            .get(&candidate)
            .map(String::as_str)
            .and_then(|value| bounded_field(value, 120))
        {
            return value;
        }
    }

    config
        .greetings
        .get(base_locale_tag(&config.fallback_locale))
        .map(String::as_str)
        .and_then(|value| bounded_field(value, 120))
        .unwrap_or_else(|| DEFAULT_GREETING_FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::realtime_call::RealtimeCallOrigin;
    use serde_json::json;

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
    fn live_call_policy_prefers_current_turn_locale_over_profile() {
        let mut profile = UserProfile::default();
        profile.set("response_locale", json!("ru"));
        let config = AgentLiveCallConfig::default();

        let policy = resolve_live_realtime_call_policy(LiveRealtimeCallPolicyInput {
            config: &config,
            current_user_text: Some("What is the weather in Batumi?"),
            chat_handoff: Some("Недавно обсуждали погоду."),
            user_profile: Some(&profile),
        });

        assert_eq!(policy.locale, "en");
        assert_eq!(policy.max_tool_iterations, 2);
        assert!(policy.excluded_tools.iter().any(|tool| tool == "shell"));
    }

    #[test]
    fn live_call_policy_uses_profile_locale_when_current_turn_is_empty() {
        let mut profile = UserProfile::default();
        profile.set("response_locale", json!("ru-RU"));
        let config = AgentLiveCallConfig::default();

        let policy = resolve_live_realtime_call_policy(LiveRealtimeCallPolicyInput {
            config: &config,
            current_user_text: None,
            chat_handoff: None,
            user_profile: Some(&profile),
        });

        assert_eq!(policy.locale, "ru-ru");
    }

    #[test]
    fn live_call_prompt_includes_locale_and_handoff() {
        let prompt = build_live_realtime_call_system_prompt(
            "Base system prompt.",
            "ru",
            Some("Recent chat summary."),
            2,
        );

        assert!(prompt.contains("Preferred response locale: ru."));
        assert!(prompt.contains("Chat-to-call handoff"));
        assert!(prompt.contains("Recent chat summary."));
    }

    #[test]
    fn bounded_live_call_speech_uses_config_budget() {
        let config = AgentLiveCallConfig {
            max_spoken_chars: 24,
            max_spoken_sentences: 1,
            ..AgentLiveCallConfig::default()
        };
        let bounded = bounded_live_realtime_call_speech(
            &config,
            "First sentence is here. Second sentence should be dropped.",
        );

        assert!(bounded.starts_with("First sentence"));
        assert!(!bounded.contains("Second sentence"));
    }

    #[test]
    fn greeting_resolves_by_locale_then_fallback() {
        let config = AgentLiveCallConfig::default();
        assert_eq!(
            default_realtime_call_answer_greeting(&config, Some("ru-RU")),
            "Привет. Я на связи."
        );
        assert_eq!(
            default_realtime_call_answer_greeting(&config, None),
            "Hello. I'm here."
        );
    }
}
