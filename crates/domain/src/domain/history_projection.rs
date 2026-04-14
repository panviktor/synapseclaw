//! Canonical markers for projected provider history.
//!
//! These markers are an internal serialization edge used when native provider
//! history needs to be represented as plain `ChatMessage` values. Semantic
//! services should parse them through this module instead of matching local
//! ad hoc strings.

pub const PROJECTED_TOOL_CALL_PREFIX: &str = "[tool-call ";
pub const PROJECTED_ASSISTANT_REASONING_PREFIX: &str = "[assistant-reasoning]\n";
pub const PROJECTED_FACT_ANCHOR_PREFIX: &str = "[fact-anchor ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectedToolCall<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub arguments: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectedFactAnchor<'a> {
    pub id: &'a str,
    pub category: &'a str,
    pub text: &'a str,
}

pub fn format_projected_assistant_reasoning(reasoning: &str) -> String {
    format!("{PROJECTED_ASSISTANT_REASONING_PREFIX}{reasoning}")
}

pub fn format_projected_tool_call(id: &str, name: &str, arguments: &str) -> String {
    format!("{PROJECTED_TOOL_CALL_PREFIX}{id}]\n{name} {arguments}")
}

pub fn format_projected_fact_anchor(id: &str, category: &str, text: &str) -> String {
    format!("{PROJECTED_FACT_ANCHOR_PREFIX}{id}]\ncategory={category}\n{text}")
}

pub fn parse_projected_tool_call(content: &str) -> Option<ProjectedToolCall<'_>> {
    let body = content.strip_prefix(PROJECTED_TOOL_CALL_PREFIX)?;
    let (id, rest) = body.split_once("]\n")?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }

    let line = rest.lines().next()?.trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim();
    if name.is_empty() {
        return None;
    }
    let arguments = parts.next().unwrap_or("").trim();

    Some(ProjectedToolCall {
        id,
        name,
        arguments,
    })
}

pub fn parse_projected_fact_anchor(content: &str) -> Option<ProjectedFactAnchor<'_>> {
    let body = content.strip_prefix(PROJECTED_FACT_ANCHOR_PREFIX)?;
    let (id, rest) = body.split_once("]\n")?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }

    let (category_line, text) = rest.split_once('\n')?;
    let category_line = category_line.trim();
    let category = category_line.strip_prefix("category=")?.trim();
    if category.is_empty() {
        return None;
    }
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    Some(ProjectedFactAnchor { id, category, text })
}

pub fn is_projected_tool_call(content: &str) -> bool {
    parse_projected_tool_call(content).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_projected_tool_call() {
        let rendered = format_projected_tool_call("call-1", "shell", r#"{"cmd":"date"}"#);
        let parsed = parse_projected_tool_call(&rendered).expect("projected tool call");

        assert_eq!(parsed.id, "call-1");
        assert_eq!(parsed.name, "shell");
        assert_eq!(parsed.arguments, r#"{"cmd":"date"}"#);
    }

    #[test]
    fn rejects_plain_text() {
        assert!(parse_projected_tool_call("shell {\"cmd\":\"date\"}").is_none());
    }

    #[test]
    fn parses_projected_fact_anchor() {
        let rendered =
            format_projected_fact_anchor("fact-1", "project", "project=Atlas branch=release");
        let parsed = parse_projected_fact_anchor(&rendered).expect("projected fact anchor");

        assert_eq!(parsed.id, "fact-1");
        assert_eq!(parsed.category, "project");
        assert_eq!(parsed.text, "project=Atlas branch=release");
    }

    #[test]
    fn rejects_plain_fact_text() {
        assert!(parse_projected_fact_anchor("project=Atlas branch=release").is_none());
    }
}
