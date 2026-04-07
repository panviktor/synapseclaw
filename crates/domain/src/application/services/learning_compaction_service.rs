//! Cheap duplicate compaction for learned episodic categories.
//!
//! This keeps precedent and failure-pattern memories from growing through
//! obvious near-duplicates. The worker keeps the newer representative and
//! removes older duplicates.

use crate::application::services::failure_similarity_service;
use crate::application::services::precedent_similarity_service;
use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryError, MemoryQuery};
use crate::ports::memory::UnifiedMemoryPort;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct DuplicateCompactionThresholds {
    pub shortlist_limit: usize,
    pub duplicate_threshold: f64,
}

impl DuplicateCompactionThresholds {
    pub fn precedent_defaults() -> Self {
        Self {
            shortlist_limit: 6,
            duplicate_threshold: 0.95,
        }
    }

    pub fn failure_pattern_defaults() -> Self {
        Self {
            shortlist_limit: 6,
            duplicate_threshold: 0.96,
        }
    }
}

pub async fn compact_near_duplicates(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    category: MemoryCategory,
    limit: usize,
    thresholds: &DuplicateCompactionThresholds,
) -> Result<usize, MemoryError> {
    let entries = mem.list_scoped(Some(&category), None, limit, false).await?;
    if entries.len() < 2 {
        return Ok(0);
    }

    let mut similarity_lookup = HashMap::new();
    for entry in &entries {
        similarity_lookup.insert(
            entry.key.clone(),
            fetch_category_shortlist(mem, agent_id, entry, &category, thresholds.shortlist_limit)
                .await?,
        );
    }

    let duplicate_keys = plan_duplicate_removals(
        &entries,
        &similarity_lookup,
        &category,
        thresholds.duplicate_threshold,
    );
    let mut removed = 0;
    for key in duplicate_keys {
        if mem.forget(&key, &agent_id.to_string()).await? {
            removed += 1;
        }
    }
    Ok(removed)
}

async fn fetch_category_shortlist(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    entry: &MemoryEntry,
    category: &MemoryCategory,
    limit: usize,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let query = MemoryQuery {
        text: entry.content.clone(),
        embedding: None,
        agent_id: agent_id.to_string(),
        categories: vec![category.clone()],
        include_shared: false,
        time_range: None,
        limit: limit.saturating_mul(2).max(limit),
    };
    let mut episodes = mem.hybrid_search(&query).await?.episodes;
    episodes.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(episodes
        .into_iter()
        .take(limit)
        .map(|result| {
            let mut entry = result.entry;
            entry.score = Some(result.score as f64);
            entry
        })
        .collect())
}

pub fn plan_duplicate_removals(
    entries: &[MemoryEntry],
    similarity_lookup: &HashMap<String, Vec<MemoryEntry>>,
    category: &MemoryCategory,
    duplicate_threshold: f64,
) -> Vec<String> {
    let mut ordered = entries.to_vec();
    ordered.sort_by(|left, right| {
        parse_timestamp(&right.timestamp)
            .cmp(&parse_timestamp(&left.timestamp))
            .then_with(|| left.key.cmp(&right.key))
    });

    let mut kept = HashSet::new();
    let mut removed = HashSet::new();

    for entry in &ordered {
        if removed.contains(&entry.key) {
            continue;
        }

        let Some(shortlist) = similarity_lookup.get(&entry.key) else {
            kept.insert(entry.key.clone());
            continue;
        };

        for similar in shortlist {
            if similar.key == entry.key || removed.contains(&similar.key) {
                continue;
            }
            if similar.score.unwrap_or(0.0) < duplicate_threshold {
                continue;
            }
            if distinct_procedural_shape(category, &entry.content, &similar.content) {
                continue;
            }
            if kept.contains(&similar.key) {
                removed.insert(entry.key.clone());
                break;
            }
            removed.insert(similar.key.clone());
        }

        if !removed.contains(&entry.key) {
            kept.insert(entry.key.clone());
        }
    }

    let mut keys = removed.into_iter().collect::<Vec<_>>();
    keys.sort();
    keys
}

fn distinct_procedural_shape(category: &MemoryCategory, left: &str, right: &str) -> bool {
    match category {
        MemoryCategory::Custom(name) if name == "precedent" => {
            precedent_similarity_service::precedent_contents_have_distinct_shape(left, right)
        }
        MemoryCategory::Custom(name) if name == "failure_pattern" => {
            failure_similarity_service::failure_contents_have_distinct_shape(left, right)
        }
        _ => false,
    }
}

fn parse_timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, timestamp: &str) -> MemoryEntry {
        MemoryEntry {
            id: key.into(),
            key: key.into(),
            content: format!("entry:{key}"),
            category: MemoryCategory::Custom("precedent".into()),
            timestamp: timestamp.into(),
            session_id: None,
            score: None,
        }
    }

    fn similar(key: &str, score: f64) -> MemoryEntry {
        MemoryEntry {
            id: key.into(),
            key: key.into(),
            content: format!("entry:{key}"),
            category: MemoryCategory::Custom("precedent".into()),
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: None,
            score: Some(score),
        }
    }

    #[test]
    fn removes_older_duplicate_when_newer_entry_kept() {
        let entries = vec![
            entry("newer", "2026-02-01T00:00:00Z"),
            entry("older", "2026-01-01T00:00:00Z"),
        ];
        let similarity_lookup = HashMap::from([
            (
                "newer".into(),
                vec![similar("newer", 1.0), similar("older", 0.98)],
            ),
            (
                "older".into(),
                vec![similar("older", 1.0), similar("newer", 0.98)],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
        );
        assert_eq!(removed, vec!["older".to_string()]);
    }

    #[test]
    fn removes_current_entry_if_duplicate_of_kept_newer_entry() {
        let entries = vec![
            entry("first", "2026-03-01T00:00:00Z"),
            entry("second", "2026-02-01T00:00:00Z"),
            entry("third", "2026-01-01T00:00:00Z"),
        ];
        let similarity_lookup = HashMap::from([
            (
                "first".into(),
                vec![similar("first", 1.0), similar("second", 0.97)],
            ),
            (
                "second".into(),
                vec![similar("second", 1.0), similar("first", 0.97)],
            ),
            ("third".into(), vec![similar("third", 1.0)]),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
        );
        assert_eq!(removed, vec!["second".to_string()]);
    }

    #[test]
    fn keeps_high_similarity_precedents_with_distinct_tool_shape() {
        let entries = vec![
            MemoryEntry {
                id: "newer".into(),
                key: "newer".into(),
                content:
                    "tools=web_search -> message_send | subjects=status.example.com | facets=send"
                        .into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: "2026-03-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            MemoryEntry {
                id: "older".into(),
                key: "older".into(),
                content: "tools=shell -> message_send | subjects=status.example.com | facets=send"
                    .into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: "2026-02-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
        ];
        let similarity_lookup = HashMap::from([
            (
                "newer".into(),
                vec![
                    similar("newer", 1.0),
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "tools=shell -> message_send | subjects=status.example.com | facets=send".into(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
            (
                "older".into(),
                vec![
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "tools=shell -> message_send | subjects=status.example.com | facets=send".into(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(1.0),
                    },
                    MemoryEntry {
                        id: "newer".into(),
                        key: "newer".into(),
                        content: "tools=web_search -> message_send | subjects=status.example.com | facets=send".into(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-03-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
        );
        assert!(removed.is_empty());
    }

    #[test]
    fn keeps_high_similarity_failure_patterns_with_distinct_failed_tools() {
        let entries = vec![
            MemoryEntry {
                id: "newer".into(),
                key: "newer".into(),
                content: "failed_tools=web_fetch -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-03-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            MemoryEntry {
                id: "older".into(),
                key: "older".into(),
                content: "failed_tools=shell -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-02-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
        ];
        let similarity_lookup = HashMap::from([
            (
                "newer".into(),
                vec![
                    MemoryEntry {
                        id: "newer".into(),
                        key: "newer".into(),
                        content: "failed_tools=web_fetch -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-03-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(1.0),
                    },
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "failed_tools=shell -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
            (
                "older".into(),
                vec![
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "failed_tools=shell -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(1.0),
                    },
                    MemoryEntry {
                        id: "newer".into(),
                        key: "newer".into(),
                        content: "failed_tools=web_fetch -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-03-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("failure_pattern".into()),
            0.95,
        );
        assert!(removed.is_empty());
    }
}
