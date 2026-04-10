#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredGenerationMarker {
    Image,
    Audio,
    Video,
    Music,
}

pub fn contains_image_attachment_marker(text: &str) -> bool {
    text.contains("[IMAGE:")
}

pub fn leading_media_control_marker(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("[GENERATE:") || trimmed.starts_with("[IMAGE:")
}

pub fn detect_generation_marker(text: &str) -> Option<StructuredGenerationMarker> {
    if text.contains("[GENERATE:IMAGE]") {
        return Some(StructuredGenerationMarker::Image);
    }
    if text.contains("[GENERATE:AUDIO]") {
        return Some(StructuredGenerationMarker::Audio);
    }
    if text.contains("[GENERATE:VIDEO]") {
        return Some(StructuredGenerationMarker::Video);
    }
    if text.contains("[GENERATE:MUSIC]") {
        return Some(StructuredGenerationMarker::Music);
    }
    None
}

pub fn strip_image_attachment_markers(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '[' {
            result.push(ch);
            continue;
        }

        let mut lookahead = String::from(ch);
        for _ in 0..6 {
            if let Some(&next) = chars.peek() {
                lookahead.push(next);
                chars.next();
            } else {
                break;
            }
        }

        if lookahead.starts_with("[IMAGE:") {
            for c in chars.by_ref() {
                if c == ']' {
                    break;
                }
            }
            continue;
        }

        result.push_str(&lookahead);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_generation_markers() {
        assert_eq!(
            detect_generation_marker("[GENERATE:VIDEO] short trailer"),
            Some(StructuredGenerationMarker::Video)
        );
        assert_eq!(
            detect_generation_marker("[GENERATE:MUSIC] menu theme"),
            Some(StructuredGenerationMarker::Music)
        );
        assert_eq!(
            detect_generation_marker("[GENERATE:IMAGE] album cover"),
            Some(StructuredGenerationMarker::Image)
        );
        assert_eq!(
            detect_generation_marker("[GENERATE:AUDIO] narration"),
            Some(StructuredGenerationMarker::Audio)
        );
        assert_eq!(detect_generation_marker("plain text"), None);
    }

    #[test]
    fn strips_image_markers_but_preserves_normal_brackets() {
        assert_eq!(
            strip_image_attachment_markers("Hello [IMAGE:abc123] world"),
            "Hello  world"
        );
        assert_eq!(
            strip_image_attachment_markers("Hello [world] test"),
            "Hello [world] test"
        );
    }

    #[test]
    fn leading_media_marker_only_matches_structured_prefix() {
        assert!(leading_media_control_marker("[GENERATE:IMAGE] cover"));
        assert!(leading_media_control_marker(
            "[IMAGE:data:image/png;base64,abc]"
        ));
        assert!(!leading_media_control_marker("Describe [IMAGE:cat]"));
    }
}
