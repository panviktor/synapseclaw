//! Utility functions and types shared across the codebase.

use std::collections::HashMap;

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
///
/// Safely handles multi-byte UTF-8 characters (emoji, CJK, accented characters)
/// by using character boundaries instead of byte indices.
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => {
            let truncated = &s[..idx];
            format!("{}...", truncated.trim_end())
        }
        None => s.to_string(),
    }
}

/// Redact a sensitive value, keeping the first 4 characters.
pub fn redact(value: &str) -> String {
    let char_count = value.chars().count();
    if char_count <= 4 {
        "***".to_string()
    } else {
        let prefix: String = value.chars().take(4).collect();
        format!("{prefix}***")
    }
}

/// Check if content should be skipped for autosave (cron prefixes, distillation markers).
pub fn should_skip_autosave_content(content: &str) -> bool {
    let normalized = content.trim();
    if normalized.is_empty() {
        return true;
    }
    let lowered = normalized.to_ascii_lowercase();
    lowered.starts_with("[cron:")
        || lowered.starts_with("[distilled_")
        || lowered.contains("distilled_index_sig:")
}

/// Detect long low-information repetition so autosave/consolidation do not
/// preserve obvious chant-like noise just because it exceeds the minimum length.
pub fn is_low_information_repetition(content: &str) -> bool {
    let normalized = content.trim();
    if normalized.is_empty() {
        return false;
    }

    let tokens: Vec<String> = normalized
        .split_whitespace()
        .filter_map(normalize_repetition_token)
        .collect();
    if tokens.len() < 12 {
        return false;
    }

    let mut token_counts: HashMap<&str, usize> = HashMap::new();
    for token in &tokens {
        *token_counts.entry(token.as_str()).or_insert(0) += 1;
    }
    let unique_ratio = token_counts.len() as f32 / tokens.len() as f32;
    let max_token_ratio =
        token_counts.values().copied().max().unwrap_or(0) as f32 / tokens.len() as f32;

    let lines: Vec<String> = normalized
        .lines()
        .map(|line| line.trim().to_ascii_lowercase())
        .filter(|line| !line.is_empty())
        .collect();
    let repetitive_lines = if lines.len() >= 3 {
        let mut line_counts: HashMap<&str, usize> = HashMap::new();
        for line in &lines {
            *line_counts.entry(line.as_str()).or_insert(0) += 1;
        }
        let max_line_ratio =
            line_counts.values().copied().max().unwrap_or(0) as f32 / lines.len() as f32;
        max_line_ratio >= 0.6
    } else {
        false
    };

    let repetitive_token_pattern = has_repeated_token_pattern(&tokens, 2, 8, 3);
    let repetitive_semantic_shingles =
        repeated_shingle_ratio(&tokens, 2) >= 0.45 || repeated_shingle_ratio(&tokens, 3) >= 0.35;

    repetitive_lines
        || repetitive_token_pattern
        || (unique_ratio < 0.35 && max_token_ratio >= 0.24)
        || (unique_ratio < 0.62 && repetitive_semantic_shingles)
}

fn has_repeated_token_pattern(
    tokens: &[String],
    min_pattern_len: usize,
    max_pattern_len: usize,
    min_repeats: usize,
) -> bool {
    if tokens.len() < min_pattern_len.saturating_mul(min_repeats) {
        return false;
    }

    let max_pattern_len = max_pattern_len.min(tokens.len() / min_repeats);
    for pattern_len in min_pattern_len..=max_pattern_len {
        let max_start = tokens.len().saturating_sub(pattern_len * min_repeats);
        for start in 0..=max_start {
            let pattern = &tokens[start..start + pattern_len];
            let mut repeats = 1;
            let mut cursor = start + pattern_len;
            while cursor + pattern_len <= tokens.len()
                && tokens[cursor..cursor + pattern_len] == *pattern
            {
                repeats += 1;
                cursor += pattern_len;
            }
            if repeats >= min_repeats {
                return true;
            }
        }
    }

    false
}

fn normalize_repetition_token(token: &str) -> Option<String> {
    let normalized = token
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn repeated_shingle_ratio(tokens: &[String], width: usize) -> f32 {
    if width == 0 || tokens.len() < width.saturating_mul(3) {
        return 0.0;
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for window in tokens.windows(width) {
        *counts.entry(window.join("\u{1f}")).or_insert(0) += 1;
    }

    let total = tokens.len().saturating_sub(width).saturating_add(1);
    if total == 0 {
        return 0.0;
    }
    let repeated = counts
        .values()
        .copied()
        .filter(|count| *count > 1)
        .sum::<usize>();
    repeated as f32 / total as f32
}

/// Utility enum for handling optional values with three states.
pub enum MaybeSet<T> {
    Set(T),
    Unset,
    Null,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_ascii_no_truncation() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
        assert_eq!(truncate_with_ellipsis("hello world", 50), "hello world");
    }

    #[test]
    fn test_truncate_ascii_with_truncation() {
        assert_eq!(truncate_with_ellipsis("hello world", 5), "hello...");
        assert_eq!(
            truncate_with_ellipsis("This is a long message", 10),
            "This is a..."
        );
    }

    #[test]
    fn test_truncate_empty_string() {
        assert_eq!(truncate_with_ellipsis("", 10), "");
    }

    #[test]
    fn test_truncate_at_exact_boundary() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_emoji_single() {
        let s = "🦀";
        assert_eq!(truncate_with_ellipsis(s, 10), s);
        assert_eq!(truncate_with_ellipsis(s, 1), s);
    }

    #[test]
    fn test_truncate_emoji_multiple() {
        let s = "😀😀😀😀";
        assert_eq!(truncate_with_ellipsis(s, 2), "😀😀...");
        assert_eq!(truncate_with_ellipsis(s, 3), "😀😀😀...");
    }

    #[test]
    fn test_truncate_mixed_ascii_emoji() {
        assert_eq!(truncate_with_ellipsis("Hello 🦀 World", 8), "Hello 🦀...");
        assert_eq!(truncate_with_ellipsis("Hi 😊", 10), "Hi 😊");
    }

    #[test]
    fn test_truncate_cjk_characters() {
        let s = "这是一个测试消息用来触发崩溃的中文";
        let result = truncate_with_ellipsis(s, 16);
        assert!(result.ends_with("..."));
        assert!(result.is_char_boundary(result.len() - 1));
    }

    #[test]
    fn test_truncate_accented_characters() {
        let s = "café résumé naïve";
        assert_eq!(truncate_with_ellipsis(s, 10), "café résum...");
    }

    #[test]
    fn test_truncate_unicode_edge_case() {
        let s = "aé你好🦀";
        assert_eq!(truncate_with_ellipsis(s, 3), "aé你...");
    }

    #[test]
    fn test_truncate_zero_max_chars() {
        assert_eq!(truncate_with_ellipsis("hello", 0), "...");
    }

    #[test]
    fn low_information_repetition_detects_repeated_chant() {
        let text = "echo echo echo echo echo echo echo echo echo echo echo echo echo";
        assert!(is_low_information_repetition(text));
    }

    #[test]
    fn low_information_repetition_ignores_normal_semantic_paragraph() {
        let text = "Мне кажется, смысл жизни связан не с одной целью, а с тем, как человек строит отношения, труд и внимание к другим людям.";
        assert!(!is_low_information_repetition(text));
    }

    #[test]
    fn low_information_repetition_detects_repeated_multiword_pattern() {
        let text = "I want peace and meaning I want peace and meaning I want peace and meaning";
        assert!(is_low_information_repetition(text));
    }

    #[test]
    fn low_information_repetition_detects_semantic_shingle_loop() {
        let text = "meaning comes from choice and purpose comes from choice because meaning grows from choice and purpose grows from choice because meaning comes from choice";
        assert!(is_low_information_repetition(text));
    }

    #[test]
    fn low_information_repetition_ignores_distinct_long_argument() {
        let text = "Мне кажется, смысл жизни нельзя свести к одному правилу: человек сначала учится замечать важное, потом выбирает ответственность, а затем уже строит труд, дружбу и любовь вокруг этого выбора.";
        assert!(!is_low_information_repetition(text));
    }
}
