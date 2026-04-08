pub(crate) fn is_low_signal_bootstrap_path(path: &str) -> bool {
    let raw = path.trim().replace('\\', "/");
    if raw.is_empty() {
        return true;
    }

    is_session_archive(&raw.to_ascii_lowercase()) || is_root_bootstrap_markdown(&raw)
}

pub(crate) fn preferred_workspace_locator<'a, I>(paths: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut first = None;
    for path in paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        if first.is_none() {
            first = Some(trimmed.to_string());
        }
        if !is_low_signal_bootstrap_path(trimmed) {
            return Some(trimmed.to_string());
        }
    }
    first
}

pub(crate) fn truncate_fact_value(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn is_session_archive(path: &str) -> bool {
    path.ends_with(".jsonl") && path.split('/').any(|segment| segment == "sessions")
}

fn is_root_bootstrap_markdown(path: &str) -> bool {
    let trimmed = path.trim_start_matches("./");
    if trimmed.contains('/') || !trimmed.ends_with(".md") {
        return false;
    }

    let stem = trimmed.trim_end_matches(".md");
    !stem.is_empty()
        && stem.len() <= 16
        && stem
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}
