//! Message routing domain types.
//!
//! Phase 4.1 Slice 6: deterministic rule-based routing for inbound messages.
//! Rules are evaluated in priority order; first match wins.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// A complete routing configuration, typically loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingTable {
    /// Ordered list of routes (lower priority number = higher precedence).
    pub routes: Vec<Route>,
    /// Agent ID for messages that match no route.
    pub fallback: String,
}

impl RoutingTable {
    /// Evaluate all routes against an input, return the target agent_id.
    /// Returns the fallback if no route matches.
    pub fn resolve(&self, input: &RoutingInput) -> RoutingResult {
        for route in &self.routes {
            if route.rule.matches(input) {
                return RoutingResult {
                    target: route.target.clone(),
                    matched_rule: Some(route.name.clone()),
                    is_fallback: false,
                };
            }
        }
        RoutingResult {
            target: self.fallback.clone(),
            matched_rule: None,
            is_fallback: true,
        }
    }
}

/// A single routing rule with target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    /// Human-readable name for logging.
    pub name: String,
    /// The matching rule.
    pub rule: RoutingRule,
    /// Agent ID to route to on match.
    pub target: String,
    /// Priority (lower = higher precedence). Routes sorted by this.
    #[serde(default)]
    pub priority: u16,
}

/// Result of routing evaluation.
#[derive(Debug, Clone)]
pub struct RoutingResult {
    /// Target agent ID.
    pub target: String,
    /// Name of the matched rule (None if fallback).
    pub matched_rule: Option<String>,
    /// Whether the fallback was used.
    pub is_fallback: bool,
}

/// Input data for route matching.
#[derive(Debug, Clone)]
pub struct RoutingInput {
    /// Message content text.
    pub content: String,
    /// Source kind (channel, ipc, web, cron).
    pub source_kind: String,
    /// Metadata fields (key-value).
    pub metadata: std::collections::HashMap<String, String>,
}

/// Matching rule for a route.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingRule {
    /// Exact command prefix match (e.g. "/research").
    Command(String),
    /// Substring match on message content (will become regex when `regex` crate added).
    #[serde(alias = "regex")]
    Substring(String),
    /// Any keyword present in message content (case-insensitive).
    Keywords(Vec<String>),
    /// Metadata field equals value.
    FieldEquals { field: String, value: String },
    /// Source kind matches.
    SourceKind(String),
    /// Always matches (useful for catch-all before fallback).
    Always,
}

impl RoutingRule {
    /// Check if this rule matches the input.
    pub fn matches(&self, input: &RoutingInput) -> bool {
        match self {
            Self::Command(cmd) => {
                let trimmed = input.content.trim();
                trimmed == cmd || trimmed.starts_with(&format!("{cmd} "))
            }
            Self::Substring(pattern) => input.content.contains(pattern),
            Self::Keywords(keywords) => {
                let lower = input.content.to_lowercase();
                keywords.iter().any(|kw| lower.contains(&kw.to_lowercase()))
            }
            Self::FieldEquals { field, value } => {
                input.metadata.get(field).map_or(false, |v| v == value)
            }
            Self::SourceKind(kind) => input.source_kind == *kind,
            Self::Always => true,
        }
    }
}

impl fmt::Display for RoutingRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(cmd) => write!(f, "command:{cmd}"),
            Self::Substring(pat) => write!(f, "substring:{pat}"),
            Self::Keywords(kw) => write!(f, "keywords:[{}]", kw.join(",")),
            Self::FieldEquals { field, value } => write!(f, "field:{field}={value}"),
            Self::SourceKind(kind) => write!(f, "source:{kind}"),
            Self::Always => write!(f, "always"),
        }
    }
}

// ---------------------------------------------------------------------------
// TOML wrapper
// ---------------------------------------------------------------------------

/// Top-level TOML structure for routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingToml {
    #[serde(default)]
    pub routes: Vec<Route>,
    pub fallback: String,
}

impl RoutingToml {
    /// Convert to a RoutingTable, sorting routes by priority.
    pub fn into_table(mut self) -> RoutingTable {
        self.routes.sort_by_key(|r| r.priority);
        RoutingTable {
            routes: self.routes,
            fallback: self.fallback,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn input(content: &str) -> RoutingInput {
        RoutingInput {
            content: content.into(),
            source_kind: "channel".into(),
            metadata: HashMap::new(),
        }
    }

    fn input_with_meta(content: &str, key: &str, val: &str) -> RoutingInput {
        let mut meta = HashMap::new();
        meta.insert(key.into(), val.into());
        RoutingInput {
            content: content.into(),
            source_kind: "channel".into(),
            metadata: meta,
        }
    }

    fn input_with_source(content: &str, source: &str) -> RoutingInput {
        RoutingInput {
            content: content.into(),
            source_kind: source.into(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn command_match() {
        let rule = RoutingRule::Command("/research".into());
        assert!(rule.matches(&input("/research topic")));
        assert!(rule.matches(&input("/research")));
        assert!(!rule.matches(&input("/researcher")));
        assert!(!rule.matches(&input("do /research")));
    }

    #[test]
    fn keywords_case_insensitive() {
        let rule = RoutingRule::Keywords(vec!["deploy".into(), "restart".into()]);
        assert!(rule.matches(&input("please Deploy the server")));
        assert!(rule.matches(&input("need to RESTART")));
        assert!(!rule.matches(&input("write a blog post")));
    }

    #[test]
    fn substring_match() {
        let rule = RoutingRule::Substring("PR #".into());
        assert!(rule.matches(&input("review PR #123")));
        assert!(!rule.matches(&input("no match here")));
    }

    #[test]
    fn field_equals() {
        let rule = RoutingRule::FieldEquals {
            field: "kind".into(),
            value: "task".into(),
        };
        assert!(rule.matches(&input_with_meta("", "kind", "task")));
        assert!(!rule.matches(&input_with_meta("", "kind", "text")));
        assert!(!rule.matches(&input("")));
    }

    #[test]
    fn source_kind() {
        let rule = RoutingRule::SourceKind("cron".into());
        assert!(rule.matches(&input_with_source("", "cron")));
        assert!(!rule.matches(&input_with_source("", "channel")));
    }

    #[test]
    fn always_matches() {
        assert!(RoutingRule::Always.matches(&input("anything")));
    }

    #[test]
    fn routing_table_first_match_wins() {
        let table = RoutingTable {
            routes: vec![
                Route {
                    name: "research".into(),
                    rule: RoutingRule::Command("/research".into()),
                    target: "news-reader".into(),
                    priority: 10,
                },
                Route {
                    name: "catch-all".into(),
                    rule: RoutingRule::Always,
                    target: "general".into(),
                    priority: 100,
                },
            ],
            fallback: "marketing-lead".into(),
        };

        let r1 = table.resolve(&input("/research AI"));
        assert_eq!(r1.target, "news-reader");
        assert_eq!(r1.matched_rule, Some("research".into()));
        assert!(!r1.is_fallback);

        let r2 = table.resolve(&input("hello"));
        assert_eq!(r2.target, "general"); // catch-all, not fallback
        assert!(!r2.is_fallback);
    }

    #[test]
    fn routing_table_fallback() {
        let table = RoutingTable {
            routes: vec![Route {
                name: "commands".into(),
                rule: RoutingRule::Command("/cmd".into()),
                target: "bot".into(),
                priority: 10,
            }],
            fallback: "default-agent".into(),
        };

        let r = table.resolve(&input("random message"));
        assert_eq!(r.target, "default-agent");
        assert!(r.is_fallback);
    }

    #[test]
    fn toml_parsing() {
        let toml_str = r#"
fallback = "marketing-lead"

[[routes]]
name = "research-cmd"
target = "news-reader"
priority = 10

[routes.rule]
command = "/research"

[[routes]]
name = "deploy-keywords"
target = "devops"
priority = 20

[routes.rule]
keywords = ["deploy", "restart"]

[[routes]]
name = "cron-jobs"
target = "scheduler"
priority = 30

[routes.rule]
source_kind = "cron"
"#;
        let parsed: RoutingToml = toml::from_str(toml_str).unwrap();
        let table = parsed.into_table();

        assert_eq!(table.fallback, "marketing-lead");
        assert_eq!(table.routes.len(), 3);
        assert_eq!(table.routes[0].name, "research-cmd"); // priority 10
        assert_eq!(table.routes[1].name, "deploy-keywords"); // priority 20
        assert_eq!(table.routes[2].name, "cron-jobs"); // priority 30

        let r = table.resolve(&input("/research trends"));
        assert_eq!(r.target, "news-reader");
    }

    #[test]
    fn priority_sorting() {
        let toml = RoutingToml {
            routes: vec![
                Route {
                    name: "low".into(),
                    rule: RoutingRule::Always,
                    target: "c".into(),
                    priority: 99,
                },
                Route {
                    name: "high".into(),
                    rule: RoutingRule::Always,
                    target: "a".into(),
                    priority: 1,
                },
                Route {
                    name: "mid".into(),
                    rule: RoutingRule::Always,
                    target: "b".into(),
                    priority: 50,
                },
            ],
            fallback: "x".into(),
        };
        let table = toml.into_table();
        assert_eq!(table.routes[0].target, "a");
        assert_eq!(table.routes[1].target, "b");
        assert_eq!(table.routes[2].target, "c");
    }
}
